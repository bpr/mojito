# expect: no overload matches
# A `List[ref T]` element write reaches the stored referent (augmented writes
# and method calls); replacing the stored handle itself through a subscript
# assignment is not offered, so no frame-local handle can be smuggled into an
# outliving reference collection.
@fieldwise_init
struct RefList[origin: Origin[mut=True]]:
    var values: List[ref[origin] Int]
    def swap(mut self):
        var local = 9
        ref view = local
        self.values[0] = view

def main():
    print(1)
