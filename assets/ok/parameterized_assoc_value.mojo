# A value-parameterized associated type: `comptime Buf[n: Int]: AnyType`, applied
# as `Self.Buf[8]`. The single explicit parameter is supplied at the application.
trait HasBuf:
    comptime Buf[n: Int]: AnyType

    def get(self) -> Self.Buf[8]:
        ...

def main():
    print(42)
