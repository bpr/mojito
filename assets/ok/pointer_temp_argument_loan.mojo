# A temporary constructor result whose pointer field borrows caller storage
# stays anchored across the call it feeds: the hidden argument slot's loan
# keeps `n` alive until `read` has run.
@fieldwise_init
struct Holder[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Int, Self.o]

def read(h: Holder) -> Int:
    return h.src[]

def main():
    var n = 7
    print(read(Holder(Pointer(to=n))))
