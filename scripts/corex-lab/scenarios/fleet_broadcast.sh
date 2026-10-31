FIXTURE=python-calc; YOLO=1
PROMPT="Con fleet_broadcast_command ejecuta 'uname -sr' en todos los nodos (targets all) y dime qué kernel tiene cada uno."
CHECK='grep -A1 "^  fleet_broadcast_command" "$TRACE" | grep -q "Linux" && grep -c "lab-" "$LOG" >/dev/null'
