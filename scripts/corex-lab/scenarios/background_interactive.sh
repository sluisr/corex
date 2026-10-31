FIXTURE=python-calc; YOLO=1
PROMPT="Arranca python3 -i -q en segundo plano con run_shell_command (is_background true). Envíale con write_background_input la línea print(6*7) y luego lee su salida con read_background_output. Dime el número que imprimió y mata el proceso."
CHECK='grep -q "write_background_input" "$TRACE" && grep -A1 "read_background_output" "$TRACE" | grep -q 42'
