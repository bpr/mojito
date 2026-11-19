# A construction whose bracket disagrees with the result an initializer
# declares (`-> W[x.n + 1]`) is no candidate, as for an `out self: W[…]`.
# expect: type mismatch for parameter 'n' of the return type 'W[3]'
struct W[n: Int]:
    def __init__(out self):
        pass

    def __init__(x: W[_]) -> W[x.n + 1]:
        return {}


def main():
    print(W[7](W[2]()).n)
