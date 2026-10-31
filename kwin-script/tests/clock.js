module.exports = function createClock(signal) {
    let now = 0;
    const timers = [];
    function QTimer() {
        this.timeout = signal();
        this.active = false;
        this.start = () => { this.active = true; this.deadline = now + this.interval; };
        this.stop = () => { this.active = false; };
        timers.push(this);
    }
    function advance(milliseconds) {
        const target = now + milliseconds;
        for (;;) {
            const timer = timers.filter(timer => timer.active && timer.deadline <= target)
                .sort((left, right) => left.deadline - right.deadline)[0];
            if (!timer)
                break;
            now = timer.deadline;
            if (timer.singleShot)
                timer.active = false;
            else
                timer.deadline += timer.interval;
            timer.timeout.emit();
        }
        now = target;
    }
    return {QTimer, advance, now: () => now};
};
