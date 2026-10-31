FIXTURE=rust-mini; YOLO=1
PROMPT="cargo test falla. Arregla los bugs de src/lib.rs (sin cambiar los tests) y ejecuta cargo test hasta que pase."
CHECK='cargo test -q 2>&1 | grep -q "test result: ok"'
