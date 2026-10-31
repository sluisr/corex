FIXTURE=python-calc; YOLO=1
PROMPT="Lanza EN SEGUNDO PLANO (background: true) dos subagentes con delegate_subagent: un scout que describa el proyecto y un tester que ejecute python3 -m unittest. Mientras corren, consulta su estado con manage_subagents hasta que terminen y resume lo que reportó cada uno."
CHECK='grep -q "manage_subagents" "$TRACE" && [ "$(grep -c "^  manage_subagents" "$TRACE")" -le 4 ]'
