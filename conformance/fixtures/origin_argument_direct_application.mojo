# Direct origin application on a struct (`Holder[origin_of(n)]`) — the
# upstream-attested spelling, accepted by Mojito's 2026-08
# validate-then-erase origin arguments. Both compilers print 7
# (confirmed against the a79bdf59f2 pin, 2026-08-27).
@fieldwise_init
struct Holder[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Int, Self.o]

def main():
    var n = 7
    var h: Holder[origin_of(n)] = Holder(Pointer(to=n))
    print(h.src[])
