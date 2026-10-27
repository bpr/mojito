# A reflection query is the check's wherever it is spelled: a module
# constant over a struct declared before its fields resolve, a struct's own
# member over `reflect[Self]` (answered once the fields are, as upstream's
# member is lazy), and a module-scope handle read in a body directly or
# through a local binding.
@fieldwise_init
struct P:
    var x: Int
    var y: Float64
    comptime n = reflect[Self].field_count()

    def count(self) -> Int:
        return Self.n


comptime n = reflect[P].field_count()
comptime b = reflect[P].is_struct()
comptime i = reflect[P].field_index["y"]()
comptime R = reflect[P]


def main():
    print(n, b, i)
    print(P(1, 2.0).count(), P.n)
    print(R.field_count())
    comptime k = R.field_count()
    print(k)
