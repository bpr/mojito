# expect: conflicts with live reference
# A compile-time-parameterized free function returning an origin-bearing
# struct (`-> View[origin_of(xs)]`, minted as a `$`-mangled clone per
# instantiation) loans its parameter place exactly like the plain spelling:
# mutating the source list while the returned view lives is rejected.
@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]

    def first(self) -> Int:
        return self.src[0]

def make_view[n: Int](ref xs: List[Int]) -> View[origin_of(xs)]:
    return View(xs)

def main():
    var data: List[Int] = [3, 4]
    var v = make_view[1](data)
    data.append(5)
    print(v.first())
