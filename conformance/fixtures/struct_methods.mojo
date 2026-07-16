@fieldwise_init
struct Point:
    var x: Int
    var y: Int

    def total(self) -> Int:
        return self.x + self.y

    def shift(mut self, amount: Int):
        self.x += amount

def main():
    var point = Point(3, 4)
    print(point.total())
    point.shift(5)
    print(point.x, point.y)
