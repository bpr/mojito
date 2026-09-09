# expect: returned reference escapes storage outside its declared origin
# A view borrowing a materialized temporary is frame-local by construction:
# returning it rejects exactly like a view of an owned local.
@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]

def make_list() -> List[Int]:
    var xs = List[Int]()
    xs.append(7)
    return xs^

def make_view() -> View[MutUnsafeAnyOrigin]:
    return View(make_list())

def main():
    var v = make_view()
    print(v.src[0])
