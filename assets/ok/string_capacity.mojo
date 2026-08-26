# String stabilizations (upstream 2026-08): the bare empty constructor,
# keyword `capacity_bytes` construction, and `reserve_bytes` (a no-op when
# capacity already suffices; capacity is a real byte buffer in the
# self-hosted String).
def main():
    var s = String()
    s += "x"
    print(s)
    var t = String(capacity_bytes=64)
    t += "abc"
    t.reserve_bytes(128)
    t += "def"
    print(t)
