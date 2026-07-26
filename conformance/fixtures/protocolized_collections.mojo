from std.collections.set import Set

def main() raises:
    var values: List[_] = [1, 2, 3]
    var unique: Set[_] = {3, 1, 3, 2}
    var mapping: Dict[String, _] = {"one": 1, "two": 2}

    values.append(4)
    mapping["one"] = 9
    print(len(values), values[3], 2 in unique)
    print(len(mapping), mapping["one"])

    var total = 0
    for value in range(1, 8, 2):
        total += value
    print(total)
