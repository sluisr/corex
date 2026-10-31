FIXTURE=python-calc; YOLO=1
PROMPT="Los tests fallan. Arregla calc.py con cambios mínimos usando apply_patch, sin tocar los tests, y ejecuta python3 -m unittest hasta que pasen."
CHECK='python3 -m unittest -q && git diff --quiet -- test_calc.py'
