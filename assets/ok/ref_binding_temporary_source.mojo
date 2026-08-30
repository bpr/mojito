# A `ref` binding of an owned temporary (upstream's temporary-lifetime rule):
# the call result is materialized into a hidden owned slot the binding
# aliases, exactly like a `ref` binding of a named place.
def make_list() -> List[Int]:
    var xs = List[Int]()
    xs.append(7)
    return xs^

def main():
    ref x = make_list()
    print(x[0])
