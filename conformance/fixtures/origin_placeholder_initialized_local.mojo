# Upstream's origin placeholder spellings (`_`, `...`) on an initialized
# local: the origin infers from the initializer. Both compilers print 7 then
# 9 (confirmed against the a79fbdf59f2 pin, 2026-08-29).
def main():
    var xs = List[Int]()
    xs.append(7)
    xs.append(9)
    var s: Span[Int, _] = xs
    print(s[0])
    var t: Span[Int, ...] = xs
    print(t[1])
