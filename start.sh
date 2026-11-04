#!/bin/bash
# Start the exchange plus two bots, so the public demo has a live market.
PORT="${PORT:-3000}"
export MATCHBOX_URL="ws://127.0.0.1:${PORT}/ws"

./server &
sleep 2
./market_maker &
./bots "${BOT_RATE:-30}" &

# if any process dies, exit so the platform restarts the whole container
wait -n
exit 1
