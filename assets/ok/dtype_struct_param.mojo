# A struct may take a [dtype: DType] value parameter: each application
# monomorphizes, so Scalar[dt] fields and signatures check concretely.
struct Cell[dt: DType]:
    var value: Scalar[dt]

    def __init__(out self, value: Scalar[dt]):
        self.value = value

    def get(self) -> Scalar[dt]:
        return self.value

def main():
    var c = Cell[DType.uint8](Scalar[DType.uint8](300))
    print(c.get())
    var f = Cell[DType.float32](Scalar[DType.float32](2.5))
    print(f.get())
