# Reading through a collection element's `ref` field — chained
# (`sink[0].value[0]`) and via an element binding — selects the subscript on
# the referent, reads through the stored handle at dispatch, and chases the
# handle mid-projection in the composed reference result. The borrowed
# source stays live past the reads.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

def main():
    var sink: List[RefBox] = List[RefBox]()
    var local: List[Int] = [9]
    ref alias = local
    sink.append(RefBox(alias))
    print(sink[0].value[0])
    ref e = sink[0]
    print(e.value[0])
    print(len(local))
