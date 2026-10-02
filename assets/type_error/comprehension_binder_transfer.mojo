# expect: cannot transfer out of immutable reference
# A comprehension binder is not a transferable place, even over an owned
# iteration; the element names the binder bare (`[x for x in items^]`).
def main():
    var items: List[String] = [String("a"), String("b")]
    var moved = [x^ for x in items^]
    print(len(moved))
