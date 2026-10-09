def make() -> Optional[Int]:
    return Optional[Int](3)

comptime O = make()

def main():
    print(O.value(), O.or_else(0), Bool(O))
