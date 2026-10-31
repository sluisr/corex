FIXTURE=python-calc; YOLO=1
PROMPT="Guarda en la memoria de ESTE proyecto (scope project) que en este repo siempre usamos 4 espacios y nunca tabs."
CHECK='grep -q "save_memory" "$TRACE" && grep -rqi "tabs" "$LAB_HOME/.corex/projects"'
