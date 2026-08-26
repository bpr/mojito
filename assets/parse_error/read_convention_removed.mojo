# expect: 'read' was removed; use 'imm'
# The legacy `read` argument convention is a hard error upstream (2026-08
# window) with this exact migration message; `imm` is the only spelling.
# (`read` stays usable as an ordinary parameter NAME: `def f(read: Int)`.)
def show(read x: Int):
    print(x)

def main():
    show(7)
