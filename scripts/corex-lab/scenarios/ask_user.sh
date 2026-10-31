FIXTURE=python-calc; YOLO=1; ANSWER=Rust
PROMPT="Antes de nada, pregúntame con la herramienta ask_user qué lenguaje prefiero y luego responde 'Prefieres X' con mi respuesta."
CHECK='grep -A1 "ask_user" "$TRACE" | grep -q "The user answered" && grep -q "Prefieres Rust" "$LOG"'
