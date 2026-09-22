# A witness whose own binder is spelled differently from the requirement's
# (`push[X: Hasher]` for `push[H: Hasher]`) satisfies the trait: a binder's
# identity is its declaration's and its spelling is not part of the shape.
from std.hashlib import Hasher


trait Sink:
    def push[H: Hasher](self, mut hasher: H):
        ...


struct Leaf(Sink, Copyable, Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def push[X: Hasher](self, mut hasher: X):
        self.n.__hash__(hasher)


struct Wrap[T: Sink & Copyable & Deinitable](Copyable, Movable, Hashable):
    var inner: Self.T

    def __init__(out self, inner: Self.T):
        self.inner = inner.copy()

    def __hash__[H: Hasher](self, mut hasher: H):
        self.inner.push(hasher)


def main():
    var w = Wrap[Leaf](Leaf(3))
    var w2 = Wrap[Leaf](Leaf(3))
    print(hash(w) == hash(w2))
