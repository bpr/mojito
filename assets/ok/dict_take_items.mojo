# take_items drains every entry into an owned iterator; after iteration the
# dictionary is empty and reusable on both compilers. (Mojito drains
# eagerly at the call, upstream lazily through the iterator — observable
# only mid-drain; see conformance dict-take-items-eager-drain.)
def main() raises:
    var d: Dict[Int, Int] = {1: 10, 2: 20, 3: 30}
    var it = d.take_items()
    for var entry in it^:
        print(entry.key, entry.value)
    print(len(d))
    d[9] = 90
    print(len(d), d[9])
