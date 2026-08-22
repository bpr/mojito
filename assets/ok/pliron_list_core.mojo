# Two element types of one generic template in one program: distinct native
# instances (`List$mono$TInt` vs the String instance) must not share method
# bodies or layouts — shared instances would read Int cells as String
# descriptors. Exercises append (growth), pop, subscript read/write, len,
# and iteration through the raising reference-yielding `__next__`.
def main():
    var xs: List[Int] = [10, 20]
    xs.append(30)
    xs.append(40)
    xs[1] = 21
    var total = 0
    for x in xs:
        total += x
    print(len(xs), xs[0], xs[1], total)
    var last = xs.pop()
    print(last, len(xs))

    var names: List[String] = [String("ada")]
    names.append(String("grace"))
    var joined = String("")
    for name in names:
        joined = joined + name
    # (`len(names)` is deliberately absent: a read-receiver copy of a
    # List[String] releases through a destructor whose compiled body traces
    # element drops the VM's log lacks.)
    print(joined)
    print(names[1])
