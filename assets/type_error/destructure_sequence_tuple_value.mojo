# expect: has no method '__getitem__'
# A sequence target reads through `__getitem__`; a `Tuple` value has none.
def main() raises:
    var t = (1, 2)
    var [a, b] = t
    print(a, b)
