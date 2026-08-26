# mojo-only (strict-subset gap): upstream Array conforms to `Defaultable`
# when T does (`Array[Int, 3]()` prints `0 0`), default-constructing every
# element. The implementation needs `Self.T()` — constructing a type
# parameter's default value in a generic body — which Mojito does not
# support yet.
def main():
    var defaults = Array[Int, 3]()
    print(defaults[0], defaults[2])
