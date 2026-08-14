# Slice subscripts `a[i:j:k]` on List/StringLiteral. Contiguous List slices
# are strict (see list_contiguous_slice_strict.mojo and the runtime_error
# fixtures); strided List slicing and StringLiteral slicing keep Python
# semantics (negative indices, optional bounds, negative step reverses). The
# nominal String's slicing is pinned by the nominal_string_* fixtures.
def mid(xs: List[Int]) -> List[Int]:
    return xs[1:3]

def main():
    var xs: List[Int] = [0, 1, 2, 3, 4]
    print(mid(xs))
    print(xs[::-1])
    print(xs[-2::1])
    var s: StringLiteral = "hello"
    print(s[1:4])
    print(s[::-1])
