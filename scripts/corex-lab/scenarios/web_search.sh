FIXTURE=python-calc; YOLO=1
PROMPT="Usa web_search para averiguar cuál es la última versión estable de Python 3 publicada en python.org y dime el número exacto con la fuente."
CHECK='grep -q "web_search" "$TRACE" && ! grep -A1 "web_search" "$TRACE" | grep -q -- "-> ERROR"'
