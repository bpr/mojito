# expect: type mismatch for variable 'a'
# A display in a parameter position is its literal constructor's
# application, so instances over different closed displays differ.
struct Tag[t: Tuple[Int, Int]]:
    def __init__(out self):
        pass


def main():
    var a: Tag[(5, 1)] = Tag[(5, 2)]()
