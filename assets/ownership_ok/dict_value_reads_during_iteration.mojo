# Value reads during key iteration are legal: a lookup refreshes only the
# sibling "value" generation, never the "element" generation the iterator
# retains.
def main() raises:
    var d = {"a": 1, "b": 2}
    var total = 0
    for k in d:
        total += d[k]
    print(total)
