# expect: escapes storage
# `List.append`'s transfer effect installs the appended carrier's loan on the
# destination in the caller's bookkeeping, so returning the collection while
# the loan roots at a local rejects.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

def make() -> List[RefBox[MutUnsafeAnyOrigin]]:
    var local: List[Int] = [9]
    ref view = local
    var sink = List[RefBox[origin_of(view)]]()
    sink.append(RefBox(view))
    return sink^

def main():
    var got = make()
