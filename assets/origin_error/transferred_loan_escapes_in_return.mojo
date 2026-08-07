# expect: escapes storage
# `List.append`'s transfer effect installs the appended carrier's loan on the
# destination in the caller's bookkeeping, so returning the collection while
# the loan roots at a local rejects.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

def make() -> List[RefBox]:
    var sink: List[RefBox] = List[RefBox]()
    var local = [9]
    ref alias = local
    sink.append(RefBox(alias))
    return sink^

def main():
    var got = make()
