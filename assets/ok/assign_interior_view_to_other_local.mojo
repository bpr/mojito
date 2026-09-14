# A call over `s`'s owned bytes assigned to another local aliases nothing.
# The pinned Mojo prints `abc abc  `.
def takes(v: StringSpan) -> String:
    return String(v)

def main():
    var s = String("abc  ")
    var t = String("")
    t = takes(s.rstrip())
    print(t, s)
