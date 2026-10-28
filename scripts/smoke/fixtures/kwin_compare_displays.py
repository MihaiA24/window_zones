import sys
companion = {}
with open(sys.argv[1], encoding="utf-8") as stream:
    for line in stream:
        fields = line.rstrip("\n").split("|")
        if len(fields) == 5:
            companion[fields[0]] = tuple(map(int, fields[1:]))
observer = {}
with open(sys.argv[2], encoding="utf-8") as stream:
    for line in stream:
        fields = line.rstrip("\n").split("|")
        if len(fields) == 5 and not all(item.lstrip("-").isdigit() for item in fields):
            observer[fields[0]] = tuple(map(int, fields[1:]))
ok = len(companion) == 2 and len(observer) == 2
details = []
for name, rect in companion.items():
    expected = observer.get(name)
    details.append(f"{name}: companion={rect} observer-maximize={expected}")
    if expected is None or rect != expected:
        ok = False
print("; ".join(details))
print("1" if ok else "0")
