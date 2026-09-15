# The `ref`-field spelling of the ordinary fixture of the same name (a kept
# Mojito extension, see docs/non-goals.md); the origin slot is bound exactly
# as there.
# Reading through a collection element's pointer field — chained
# (`sink[0].value[0]`) and via an element binding — selects the subscript on
# the dereferenced pointee, reads through the stored handle at dispatch, and
# chases the handle mid-projection in the composed reference result. The
# borrowed source stays live past the reads.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

def main():
    var local: List[Int] = [9]
    ref view = local
    var sink = List[RefBox[origin_of(view)]]()
    sink.append(RefBox(view))
    print(sink[0].value[0])
    ref e = sink[0]
    print(e.value[0])
    print(len(local))
