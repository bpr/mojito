# A compile-time application of a function with an origin binder binds
# compile-time memory's origin, as the pin binds `ComptimeOrigin`.
struct V[n: Int]:
    def __init__(out self):
        pass

def at[o: ImmOrigin](ref[o] x: Int) -> Int:
    return x + 1

comptime W = V[at(41)]

def main():
    print(materialize[at(1)]())
