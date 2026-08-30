# expect: returned reference escapes storage outside its declared origin
# Auto-borrowing an owned place into a ref ctor parameter does not license
# escape: a view borrowing a frame-local still may not be returned.
@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

def make() -> View[MutUnsafeAnyOrigin]:
    var local = List[Int]()
    local.append(7)
    return View(local, 0)

def main():
    var v = make()
