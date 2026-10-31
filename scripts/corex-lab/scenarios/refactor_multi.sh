FIXTURE=python-calc; YOLO=1
PROMPT="Renombra mean a average en todo el proyecto (definición, imports y llamadas), añade un docstring de una línea a cada función de calc.py, arregla los bugs para que pasen los tests y verifica con grep que no queda 'mean' en el código Python."
CHECK='python3 -m unittest -q && ! grep -rnw --include=*.py mean . && [ "$(grep -c "\"\"\"" calc.py)" -ge 3 ]'
