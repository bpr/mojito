# The `ref`-field spelling of the ordinary fixture of the same name (a kept
# Mojito extension, see docs/non-goals.md); the origin slot is bound exactly
# as there.
# A pointer-carrying element type in a collection: the box constructs, moves
# in, and reports length, and its origin parameter is bound to the borrowed
# source. Leaving it unbound (`List[RefBox]`) is the Mojito-only spelling the
# `erased-origin-parameter` conformance case carries, and upstream's
# exclusivity rules also refuse to pass the collection and a box over the same
# origin to one call, so the append happens in place.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

def main():
    var local: List[Int] = [9]
    ref view = local
    var sink = List[RefBox[origin_of(view)]]()
    sink.append(RefBox(view))
    print(len(sink))
