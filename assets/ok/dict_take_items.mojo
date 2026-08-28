# take_items lazily drains through a borrowed iterator: the dictionary still
# holds every entry when the iterator is created, each step moves one entry
# out (len shrinks as the drain progresses), and the dictionary is empty and
# reusable after exhaustion. Identical output on both compilers.
def main() raises:
    var d: Dict[Int, Int] = {1: 10, 2: 20, 3: 30}
    var it = d.take_items()
    print(len(d))
    for var entry in it:
        print(entry.key, entry.value, len(d))
    print(len(d))
    d[9] = 90
    print(len(d), d[9])
