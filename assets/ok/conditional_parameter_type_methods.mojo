# A parameter typed by a conditional over a value binder is, inside its
# generic body, the archetype of every trait both arms conform to: its
# methods and operators are those traits' requirements, dispatched to the
# arm a call selects.
def g[n: Int](x: Int if n > 0 else Float64):
    var z = x.copy()
    print(z)
    print(x == x, x != x, x < x)
    if x:
        print("truthy")
    print(not x)
    var s = String()
    x.write_to(s)
    print(s)
    print(abs(x))


def main():
    g[1](-3)
    g[0](2.5)
