FIXTURE=python-calc; YOLO=1
PROMPT="Modo sysadmin: lista los nodos fleet con fleet_list_nodes (check_status true). Luego en lab-web ejecuta 'uptime' y 'df -h /' con fleet_run_command y obtén la telemetría de lab-db con fleet_get_telemetry. Resume el estado de ambos."
CHECK='grep -A1 "^  fleet_run_command" "$TRACE" | grep -q "load average" && grep -A1 "^  fleet_get_telemetry" "$TRACE" | grep -q -- "-> ok"'
