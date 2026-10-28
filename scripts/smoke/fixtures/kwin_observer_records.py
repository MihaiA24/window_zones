import json, sys
payload = None
try:
    with open(sys.argv[1], encoding="utf-8") as stream:
        for line in stream:
            marker = "KDE_OBSERVER "
            if marker in line:
                try:
                    payload = json.loads(line.split(marker, 1)[1].strip())
                except json.JSONDecodeError:
                    pass
except OSError:
    pass
if payload:
    for screen in payload.get("screens", []):
        geometry = screen.get("geometry", {})
        area = screen.get("maximizeArea", {})
        print("|".join(str(geometry.get(key, "")) for key in ("x", "y", "width", "height")))
        print("|".join([
            str(screen.get("name", "")),
            *(str(area.get(key, "")) for key in ("x", "y", "width", "height")),
        ]))
