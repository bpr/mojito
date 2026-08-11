# Two simultaneous shared iterations over one source coexist: neither loop
# mutates, so no generation is invalidated.
def main() raises:
    var xs: List[Int] = [1, 2]
    var pairs = 0
    for a in xs:
        for b in xs:
            pairs += a + b
    print(pairs)
    var d = {"a": 1, "b": 2}
    var combos = 0
    for j in d:
        for k in d:
            combos += d[j] + d[k]
    print(combos)
