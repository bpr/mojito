# Dict keys/values/items views: self-iterable, non-indexable iterators.
# Identical iteration output on both compilers (Int keys — String keys
# repr differently in aggregate prints).
def main() raises:
    var d: Dict[Int, Int] = Dict[Int, Int]()
    d[1] = 10
    d[2] = 20
    d[3] = 30
    for k in d.keys():
        print(k)
    for v in d.values():
        print(v)
    for item in d.items():
        print(item.key, item.value)
    var kv = d.keys()
    for k in kv:
        print(k)
