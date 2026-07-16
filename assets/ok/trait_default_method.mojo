# Trait default method (a real body instead of `...`).
trait DefaultQuackable:
    def quack(self):
        print("Quack")

@fieldwise_init
struct Duck(DefaultQuackable):
    var age: Int

def main():
    Duck(1).quack()
