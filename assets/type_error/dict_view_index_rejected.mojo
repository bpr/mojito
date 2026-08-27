# expect: cannot be indexed
# Upstream's views are non-indexable iterators; subscripting the stored
# view rejects on both compilers.
def main() raises:
    var d: Dict[Int, Int] = Dict[Int, Int]()
    d[1] = 10
    var keys = d.keys()
    print(keys[0])
