# Two traits giving one requirement different defaults leave a conformer that
# spells neither without a default to inherit.
# expect: trait method requirement 'name' has conflicting default implementations in 'Left' and 'Right'
trait Left:
    def name(self) -> Int:
        return 1

trait Right:
    def name(self) -> Int:
        return 2

@fieldwise_init
struct Both(Left, Right):
    var x: Int

def main():
    print(Both(0).name())
