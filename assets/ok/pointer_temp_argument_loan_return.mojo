# A loan-carrying temporary argument anchors for the whole statement even when
# the consuming call is a terminator operand: a `return` value or a branch
# condition lowers outside any statement bracket, so its hidden `$arg_loan_r`
# slot is kept alive right before the terminator instead of never.
@fieldwise_init
struct Holder[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Int, Self.o]

def read(h: Holder) -> Int:
    return h.src[]

def returned(n: Int) -> Int:
    var local = n
    return read(Holder(Pointer(to=local)))

def branched(n: Int) -> Int:
    var local = n
    if read(Holder(Pointer(to=local))) == 7:
        return 1
    return 0

def main():
    print(returned(7))
    print(branched(7))
