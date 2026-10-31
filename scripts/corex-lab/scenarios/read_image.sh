FIXTURE=image; YOLO=1
PROMPT="Mira la imagen square.png con read_image y dime de qué color es y qué forma tiene."
CHECK='grep -q "read_image" "$TRACE" && grep -qiE "roj|red" "$LOG"'
