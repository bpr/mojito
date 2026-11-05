# Unpack-assigning a call whose argument views the unpacked-into variable:
# the argument's view anchor is its memory-only temporary's call-result slot,
# which dies at the call, so the unpacking store into `a` follows no live
# loan on it, as at the pin (`ab 1`).
def pair(v: StringSpan) -> Tuple[String, Int]:
    return (String(v), 1)

def main():
    var a = String("ab  ")
    var b = 0
    a, b = pair(a.rstrip())
    print(a, b)
