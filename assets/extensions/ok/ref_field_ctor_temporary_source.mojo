# A temporary auto-borrows into a ref constructor parameter (upstream's
# temporary-lifetime rule): the call result is materialized into a hidden
# owned slot whose loans give it the borrower's lifetime, so the view stays
# readable across later statements.
@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

def make_list() -> List[Int]:
    var xs = List[Int]()
    xs.append(7)
    xs.append(9)
    return xs^

def main():
    var v = View(make_list(), 0)
    print(v.src[0])
    print(v.src[1])
