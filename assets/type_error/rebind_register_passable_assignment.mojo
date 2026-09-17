# expect: expression must be mutable in assignment ('v')
# A `TrivialRegisterPassable` operand selects upstream's by-value `rebind`
# overload, whose result is not a place, so it cannot be assigned to.
def main():
    var v = 3
    rebind[Int](v) = 4
    print(v)
