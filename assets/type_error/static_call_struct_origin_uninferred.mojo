# expect: failed to infer parameter 'origin_of(_mlir_origin)' of parent struct 'V'
# A static called through the bare name of a struct whose origin binder
# none of its arguments names leaves that parent parameter uninferred.
struct V[o: ImmOrigin]:
    def __init__(out self):
        pass

    @staticmethod
    def k() -> Int:
        return 1


def main():
    print(V.k())
