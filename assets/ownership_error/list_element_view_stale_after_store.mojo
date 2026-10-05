# expect: invalidated interior reference
# A view returned by a method on a List element borrows that element's owned
# interior, so replacing the element while the view is live invalidates it.
# requires: stdlib
def main():
    var ys: List[String] = [String("q  "), String("r ")]
    var v = ys[0].rstrip()
    ys[0] = String("new")
    print(String(v))
