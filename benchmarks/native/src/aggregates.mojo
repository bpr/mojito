# Benchmark: struct + tuple + List-heavy work. Builds lists of small structs,
# mutates them through methods, and sums fields.
@fieldwise_init
struct Particle(Copyable, Movable):
    var x: Int
    var y: Int
    var vx: Int
    var vy: Int

    def advance(mut self, steps: Int):
        self.x += self.vx * steps
        self.y += self.vy * steps

    def energy(self) -> Int:
        return self.vx * self.vx + self.vy * self.vy

def make_velocity(i: Int) -> Tuple[Int, Int]:
    return (i % 17 - 8, i % 13 - 6)

def main():
    var sum_x: Int = 0
    var sum_y: Int = 0
    var sum_e: Int = 0
    var rounds: Int = 0
    while rounds < 5:
        var ps: List[Particle] = []
        var i: Int = 0
        while i < 100:
            var vx: Int = 0
            var vy: Int = 0
            vx, vy = make_velocity(i + rounds)
            ps.append(Particle(i, rounds, vx, vy))
            i += 1
        var k: Int = 0
        while k < len(ps):
            ps[k].advance(3)
            k += 1
        for j in range(len(ps)):
            sum_x += ps[j].x
            sum_y += ps[j].y
            sum_e += ps[j].energy()
        rounds += 1

    print("sum_x:", sum_x)
    print("sum_y:", sum_y)
    print("sum_e:", sum_e)
