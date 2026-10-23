# PROBE: a pointer to an element a runtime index reaches through a
# struct's `__getitem__`.
#
# **Differs.** The pin prints `2` twice. Mojito fails MIR verification with
# "dynamic element projection requires checked indexed storage, got
# List[Int]": the checker takes `l[i]` as an index projection of the struct
# itself rather than the place `__getitem__`'s reference names (R1298).
def main():
    var l: List[Int] = [1, 2, 3]
    var i = 1
    var p = Pointer(to=l[i])
    print(p[])
    var s = Span(l)
    var q = Pointer(to=s[i])
    print(q[])
