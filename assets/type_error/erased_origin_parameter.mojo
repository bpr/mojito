# expect: 'RefBox[_]' is not concrete
# A collection element type that omits its origin slot (`List[RefBox]`) is
# not concrete, as at the pin: an origin slot infers only at the outermost
# application of a parameter or initialized-local annotation.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: Pointer[List[Int], Self.origin]

def stash(mut sink: List[RefBox], var box: RefBox):
    sink.append(box^)

def main():
    var sink = List[RefBox]()
    var local: List[Int] = [9]
    ref view = local
    stash(sink, RefBox(Pointer(to=view)))
    print(len(sink))
