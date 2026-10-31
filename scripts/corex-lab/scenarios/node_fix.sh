FIXTURE=node-mini; YOLO=1
PROMPT="node --test falla. Arregla slug.js sin tocar slug.test.js y ejecuta node --test hasta que pase."
CHECK='node --test >/dev/null 2>&1 && git diff --quiet -- slug.test.js'
