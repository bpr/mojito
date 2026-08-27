# Documented output divergence: take_items drains eagerly in Mojito (the
# dictionary is observably empty as soon as it returns, printing 0), while
# upstream's borrowed iterator drains lazily (len(d) still prints 3 before
# the iterator runs). End states agree; only the mid-drain observation
# differs.
def main() raises:
    var d: Dict[Int, Int] = {1: 10, 2: 20, 3: 30}
    var it = d.take_items()
    print(len(d))
    for var entry in it^:
        print(entry.key, entry.value)
    print(len(d))
