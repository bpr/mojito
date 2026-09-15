# expect: aliasing values passed mutably to 'sink' argument and passed mutably to 'box' argument in 'stash' call
# Two parameters naming the same origin binder reach the same storage: the
# collection carries boxes over `view` mutably and the box does too, so one
# call may not receive both, as at the pin. (`sink.append(box)` over a
# generic `T` names no origin and is accepted.)
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: Pointer[List[Int], Self.origin]

def stash[o: Origin[mut=True]](mut sink: List[RefBox[o]], var box: RefBox[o]):
    sink.append(box^)

def main():
    var local: List[Int] = [9]
    ref view = local
    var sink = List[RefBox[origin_of(view)]]()
    stash(sink, RefBox(Pointer(to=view)))
    print(len(sink))
