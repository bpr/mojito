# Array's `Deinitable` conformance is conditional on the element: an array of
# non-Deinitable elements is linear, so abandoning it at scope end rejects.
# expect: explicit-destroy obligation
@fieldwise_init
struct Res(Deinitable where False, Movable):
    var id: Int

def main():
    var a: Array[Res, 1] = [Res(1)]
    print(a[0].id)
