# A `TrivialRegisterPassable` struct transferred out of a `mut` parameter.
# The pin copies it and prints `3 3`; Mojito rejects the body ("'v' is
# uninitialized at return from this function"). `docs/roadmap.md` 3.107. When
# Mojito runs it, move this to `assets/ok/`.
@fieldwise_init
struct V(TrivialRegisterPassable):
    var n: Int

def take(mut v: V) -> V:
    return v^

def main():
    var v = V(3)
    print(take(v).n, v.n)
