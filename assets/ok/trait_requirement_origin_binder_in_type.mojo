# A trait requirement whose own origin binder an argument's type names
# (`Pointer[Int, o]`) declares, as at the pin.
trait Reader:
    def get(self) -> Int: ...

    def read[o: ImmOrigin](self, p: Pointer[Int, o]) -> Int: ...


def main():
    print(1)
