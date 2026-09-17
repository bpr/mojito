# expect: expression must be mutable for in-place operator destination ('v')
# A `TrivialRegisterPassable` operand selects upstream's by-value `rebind`
# overload, whose result is not a place, so it cannot be an in-place
# operator's destination.
def main():
    var v = 3
    rebind[Int](v) += 1
    print(v)
