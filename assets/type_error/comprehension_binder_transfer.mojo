# expect: expression does not designate a value with an origin
# A comprehension binder not declared `var` binds each element by
# reference, even over an owned iteration, so it is no tracked root a `^`
# may move out of; the element names the binder bare (`[x for x in items^]`).
def main():
    var items: List[String] = [String("a"), String("b")]
    var moved = [x^ for x in items^]
    print(len(moved))
