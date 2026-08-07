# A bare struct name as an explicit type argument (`List[RefBox]()`) resolves
# as a type argument even though the parser encodes it as a value expression:
# the checker marks it erased, so MIR emits no runtime register for it. The
# origin-erased carrier element type constructs, moves in, and reports length.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

def stash(mut sink: List[RefBox], var box: RefBox):
    sink.append(box^)

def main():
    var sink: List[RefBox] = List[RefBox]()
    var local = [9]
    ref alias = local
    stash(sink, RefBox(alias))
    print(len(sink))
