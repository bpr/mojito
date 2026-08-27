# expect: type mismatch for argument to 'len'
# Upstream's views are non-indexable iterators without len; Mojito's
# snapshot iterators expose the same rejection surface.
def main() raises:
    var d: Dict[Int, Int] = Dict[Int, Int]()
    d[1] = 10
    print(len(d.keys()))
