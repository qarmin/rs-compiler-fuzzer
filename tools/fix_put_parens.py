#!/usr/bin/env python3
"""Dev helper for the stdlib tables: `h.put(&a == b)` parses as `(h.put(&a)) == b`; wrap the argument in parentheses
whenever it has a binary operator at the top level."""
import re, sys

OPS = [" == ", " != ", " <= ", " >= ", " < ", " > ", " as ", " + ", " - ", " * ", " / ", " % ", " & ", " | ", " ^ ", " && ", " || ", " << ", " >> "]


def fix(text):
    out, i = [], 0
    key = "h.put(&"
    while True:
        j = text.find(key, i)
        if j < 0:
            out.append(text[i:])
            break
        out.append(text[i:j + len(key)])
        k = j + len(key)
        depth, instr, e = 0, False, k
        top_ops = False
        while e < len(text):
            c = text[e]
            if text.startswith('\\"', e):
                instr = not instr
                e += 2
                continue
            if instr:
                e += 1
                continue
            if c in "([{":
                depth += 1
            elif c in ")]}":
                if depth == 0:
                    break
                depth -= 1
            elif depth == 0:
                for op in OPS:
                    if text.startswith(op, e):
                        top_ops = True
            e += 1
        inner = text[k:e]
        if top_ops and not (inner.startswith("(") and inner.endswith(")") and re.fullmatch(r"\((?:[^()]|\([^()]*\))*\)", inner)):
            out.append("(" + fix(inner) + ")")
        else:
            out.append(fix(inner))
        i = e
    return "".join(out)


for p in sys.argv[1:]:
    s = open(p).read()
    n = fix(s)
    if n != s:
        open(p, "w").write(n)
        print("fixed", p)
