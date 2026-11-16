# sample-game

A dependency-free terminal game used by the GameDist integration tests and as
a publishing example.

    # Assemble a build directory: the executable plus assets/.
    cargo build -p sample-game --release
    mkdir -p build && cp target/release/sample-game build/ && cp -r examples/sample-game/assets/. build/

    build/sample-game --selftest   # prints "sample-game 1.0.0 selftest ok", exits 0

`--selftest` verifies that `data/` next to the executable is complete. The
end-to-end tests install the game with the launcher and run exactly that.
