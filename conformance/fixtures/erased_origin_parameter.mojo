# `List[RefBox]` leaves the element's origin parameter unbound: Mojito erases
# it, so one collection holds boxes over any origin, while the pin demands a
# concrete origin and rejects `RefBox[_]` as a `List` element too.
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
