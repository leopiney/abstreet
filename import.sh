#!/bin/bash

cd importer;
RUST_BACKTRACE=full cargo run --verbose --features scenarios -- $@;
cd ..;
