# Upstream's temporary-lifetime rule for `ref` bindings: `ref x = make_list()`
# accepts an owned temporary, which lives as long as the binding. Both
# compilers print 7 (confirmed against the a79fbdf59f2 pin, 2026-08-29).
def make_list() -> List[Int]:
    var xs = List[Int]()
    xs.append(7)
    return xs^

def main():
    ref x = make_list()
    print(x[0])
