# expect: cannot implicitly convert 'RefBox[origin_of(local)]' value to 'RefBox[origin]'
# Unpacking into a struct field keeps the struct's origin identity: a box over
# a local does not fit a `RefBox[Self.origin]` slot (the `ref`-field spelling
# of assets/type_error/struct_binder_origin_store_mismatch.mojo).
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Pair[origin: Origin[mut=True]]:
    var a: RefBox[Self.origin]
    var b: Int

    def fill(mut self):
        var local: List[Int] = [9]
        ref view = local
        var pack = (RefBox(view), 5)
        self.a, self.b = pack^

def main():
    var keep: List[Int] = [1]
    ref whole = keep
    var pair = Pair(RefBox(whole), 0)
    pair.fill()
