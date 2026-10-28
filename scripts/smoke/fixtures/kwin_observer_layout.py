import sys
records = []
try:
    with open(sys.argv[1], encoding="utf-8") as stream:
        for line in stream:
            fields = line.rstrip("\n").split("|")
            if len(fields) == 4:
                records.append(fields)
except OSError:
    pass
outputs = [tuple(map(int, row)) for row in records if all(item.lstrip("-").isdigit() for item in row)]
ok = len(outputs) == 2 and all(width > 0 and height > 0 for _, _, width, height in outputs)
if ok:
    (ax, ay, aw, ah), (bx, by, bw, bh) = outputs
    ok = not (
        ax < bx + bw and bx < ax + aw
        and ay < by + bh and by < ay + ah
    )
print("1" if ok else "0")
