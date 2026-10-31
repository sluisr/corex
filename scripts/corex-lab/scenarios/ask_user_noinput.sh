FIXTURE=python-calc; YOLO=1; ANSWER=""
PROMPT="Pregúntame con ask_user si prefiero tabs o espacios. Si no respondo, asume la opción más razonable y dime cuál asumiste."
CHECK='grep -A1 "ask_user" "$TRACE" | grep -q "No user is available" && grep -qiE "asum|assum" "$LOG"'
