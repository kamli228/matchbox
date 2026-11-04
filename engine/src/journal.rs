use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use crate::types::Command;

/// Writer side of the journal.
pub struct Journal {
    writer: BufWriter<File>,
}

impl Journal {
    /// Open the journal for appending, creating the file if it does not exist.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            writer: BufWriter::new(file),
        })
    }

    /// Append one command and flush it to the OS.
    pub fn append(&mut self, cmd: &Command) -> io::Result<()> {
        serde_json::to_writer(&mut self.writer, cmd)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()
    }
}

/// Read all commands from the journal. A missing file means an empty journal;
/// a broken last line (crash in the middle of a write) is ignored.
pub fn replay(path: impl AsRef<Path>) -> io::Result<Vec<Command>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };

    let mut commands = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(&line) {
            Ok(cmd) => commands.push(cmd),
            Err(_) => break,
        }
    }
    Ok(commands)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::OrderBook;
    use crate::types::Side;
    use crate::workload;

    /// A fresh path in the OS temp folder, unique per test.
    fn temp_path(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("matchbox-{name}-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn missing_file_is_empty_journal() {
        let path = temp_path("missing");
        assert!(replay(&path).unwrap().is_empty());
    }

    #[test]
    fn replay_rebuilds_identical_book() {
        let path = temp_path("rebuild");
        let commands = workload::generate(5_000, 3);

        // "before the crash": process and journal every command
        let mut original = OrderBook::new();
        let mut journal = Journal::open(&path).unwrap();
        for cmd in &commands {
            journal.append(cmd).unwrap();
            original.process(cmd.clone());
        }
        drop(journal);

        // "after restart": rebuild a new book from the file
        let replayed = replay(&path).unwrap();
        assert_eq!(replayed, commands);
        let mut restored = OrderBook::new();
        for cmd in replayed {
            restored.process(cmd);
        }

        assert_eq!(restored.order_count(), original.order_count());
        for side in [Side::Buy, Side::Sell] {
            assert_eq!(
                restored.depth(side, usize::MAX),
                original.depth(side, usize::MAX)
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn torn_last_line_is_ignored() {
        let path = temp_path("torn");
        let commands = workload::generate(10, 5);
        let mut journal = Journal::open(&path).unwrap();
        for cmd in &commands {
            journal.append(cmd).unwrap();
        }
        drop(journal);
        // simulate a crash in the middle of writing the next line
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"type\":\"NewOr").unwrap();
        drop(file);

        assert_eq!(replay(&path).unwrap(), commands);
        let _ = std::fs::remove_file(&path);
    }
}
