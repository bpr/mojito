# Upstream Array conforms to `Defaultable` when T does (`Array[Int, 3]()`
# prints `0 0`), default-constructing every element through `Self.T()`.
def main():
    var defaults = Array[Int, 3]()
    print(defaults[0], defaults[2])
