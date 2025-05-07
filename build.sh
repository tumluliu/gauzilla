#!/bin/sh

wasm-pack build --release --target web

for arg in "$@"
do
  if [ "$arg" = "sfz" ]; then
    sfz -r --coi --cors -b 0.0.0.0 -p 5050
  fi
done
