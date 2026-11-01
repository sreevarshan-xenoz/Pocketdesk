//! The phone's location for this computer (GeoClue): the computer asks the
//! phone it linked for one fix, and the phone answers with it, rounded to
//! about a kilometre unless its owner and the computer both chose exact.
//! The fix goes to the computer's apps as an NMEA source (`server/src/nmea.rs`),
//! is held in memory only, and is never stored or logged: [`Fix`]'s `Debug`
//! says nothing of where it is.

use serde_json::{Value, json};

/// Computer → phone: `{precision}`, one fix please.
pub const ASK: &str = "kdeconnect.pocketdesk.phone_location.ask";

/// Phone → computer: `{lat, lon, accuracy, precision}`, or `{error}` (the
/// phone's own switch is off, it has no permission, or no fix came).
pub const FIX: &str = "kdeconnect.pocketdesk.phone_location";

/// A city fix is rounded to this many degrees: 0.01° is about 1.1 km north
/// to south, and less east to west away from the equator.
pub const CITY_STEP_DEGREES: f64 = 0.01;

/// The least accuracy a city fix claims, in metres: it is rounded to about a
/// kilometre, whatever the phone's fix was.
pub const CITY_ACCURACY_M: u32 = 1000;

/// The most accuracy a fix may claim to be worse than, in metres: a larger
/// one is held to this.
pub const WORST_ACCURACY_M: u32 = 100_000;

/// How close a fix is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Precision {
    /// Rounded to about a kilometre. The default.
    #[default]
    City,
    /// As the phone found it.
    Exact,
}

impl Precision {
    /// Its name on the wire and in the computer's settings.
    pub fn name(self) -> &'static str {
        match self {
            Precision::City => "city",
            Precision::Exact => "exact",
        }
    }

    /// The precision `name` names, in any case; city for anything else, so
    /// a word this version does not know never gives more.
    pub fn from_name(name: &str) -> Self {
        if name.trim().eq_ignore_ascii_case("exact") {
            Precision::Exact
        } else {
            Precision::City
        }
    }

    /// The closer of the two only when both are exact: what a phone with
    /// exact allowed gives a computer that asked for `self`.
    pub fn held_to(self, allowed: Precision) -> Precision {
        if self == Precision::Exact && allowed == Precision::Exact {
            Precision::Exact
        } else {
            Precision::City
        }
    }
}

/// The body of an ask.
pub fn ask_body(precision: Precision) -> Value {
    json!({ "precision": precision.name() })
}

/// The precision an ask's body names (city when it names none).
pub fn ask_precision(body: &Value) -> Precision {
    Precision::from_name(body.get("precision").and_then(Value::as_str).unwrap_or(""))
}

/// One place: degrees, and how far off it may be in metres. Held in memory
/// only; its `Debug` hides the numbers so no log line can carry them.
#[derive(Clone, Copy, PartialEq)]
pub struct Fix {
    pub lat: f64,
    pub lon: f64,
    pub accuracy_m: u32,
    pub precision: Precision,
}

impl std::fmt::Debug for Fix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Fix {{ accuracy_m: {}, precision: {:?}, .. }}",
            self.accuracy_m, self.precision
        )
    }
}

fn round_to(value: f64, step: f64) -> f64 {
    // A whole count of steps, divided back by their number a degree holds:
    // the nearest number to the decimal, with no stray digits past it.
    let per_degree = (1.0 / step).round();
    (value * per_degree).round() / per_degree
}

impl Fix {
    /// A fix as a phone sent it, or `None` when it is not a place: a
    /// missing or non-finite number, a latitude beyond the poles or a
    /// longitude beyond the date line. The accuracy is held between 1 m and
    /// [`WORST_ACCURACY_M`] (missing: the worst).
    pub fn from_body(body: &Value) -> Option<Self> {
        let number = |key: &str| body.get(key).and_then(Value::as_f64);
        let lat = number("lat").filter(|v| v.is_finite() && v.abs() <= 90.0)?;
        let lon = number("lon").filter(|v| v.is_finite() && v.abs() <= 180.0)?;
        let accuracy = number("accuracy")
            .filter(|v| v.is_finite() && *v >= 0.0)
            .map_or(WORST_ACCURACY_M, |v| {
                v.ceil().clamp(1.0, f64::from(WORST_ACCURACY_M)) as u32
            });
        let precision =
            Precision::from_name(body.get("precision").and_then(Value::as_str).unwrap_or(""));
        Some(Self {
            lat,
            lon,
            accuracy_m: accuracy,
            precision,
        })
    }

    /// The packet's body.
    pub fn body(&self) -> Value {
        json!({
            "lat": self.lat,
            "lon": self.lon,
            "accuracy": self.accuracy_m,
            "precision": self.precision.name(),
        })
    }

    /// This fix given at `precision`, never closer than it already is:
    /// city rounds both numbers to [`CITY_STEP_DEGREES`] and claims at
    /// least [`CITY_ACCURACY_M`]; exact keeps a fix as it is, but a fix
    /// already rounded stays city.
    pub fn at(&self, precision: Precision) -> Self {
        match precision.held_to(self.precision) {
            Precision::Exact => *self,
            Precision::City => Self {
                lat: round_to(self.lat, CITY_STEP_DEGREES),
                lon: round_to(self.lon, CITY_STEP_DEGREES),
                accuracy_m: self.accuracy_m.max(CITY_ACCURACY_M),
                precision: Precision::City,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made-up places only (the open sea off West Africa, and a field in
    // Asia), never a real one.
    const SEA: (f64, f64) = (1.23456, -20.98765);

    #[test]
    fn a_precision_is_city_unless_exact_is_said() {
        assert_eq!(Precision::from_name("exact"), Precision::Exact);
        assert_eq!(Precision::from_name(" EXACT "), Precision::Exact);
        assert_eq!(Precision::from_name("city"), Precision::City);
        assert_eq!(Precision::from_name("street"), Precision::City);
        assert_eq!(Precision::from_name(""), Precision::City);
        assert_eq!(Precision::default(), Precision::City);
        assert_eq!(ask_precision(&json!({})), Precision::City);
        assert_eq!(ask_precision(&ask_body(Precision::Exact)), Precision::Exact);
    }

    #[test]
    fn exact_needs_both_the_ask_and_the_phone() {
        use Precision::{City, Exact};
        assert_eq!(Exact.held_to(Exact), Exact);
        assert_eq!(Exact.held_to(City), City, "the phone allows only city");
        assert_eq!(City.held_to(Exact), City, "the computer asked for city");
        assert_eq!(City.held_to(City), City);
    }

    #[test]
    fn a_city_fix_is_rounded_to_about_a_kilometre() {
        let fix = Fix {
            lat: SEA.0,
            lon: SEA.1,
            accuracy_m: 12,
            precision: Precision::Exact,
        };
        let city = fix.at(Precision::City);
        assert_eq!((city.lat, city.lon), (1.23, -20.99));
        assert_eq!(city.accuracy_m, CITY_ACCURACY_M);
        assert_eq!(city.precision, Precision::City);
        // Never rounded to something farther off than half a step.
        assert!((city.lat - fix.lat).abs() <= CITY_STEP_DEGREES / 2.0 + 1e-9);
        assert!((city.lon - fix.lon).abs() <= CITY_STEP_DEGREES / 2.0 + 1e-9);
        // A worse accuracy than a kilometre stays.
        let loose = Fix {
            accuracy_m: 5000,
            ..fix
        };
        assert_eq!(loose.at(Precision::City).accuracy_m, 5000);
    }

    #[test]
    fn rounding_is_steady_and_meets_the_edges() {
        let fix = |lat, lon| Fix {
            lat,
            lon,
            accuracy_m: 5,
            precision: Precision::Exact,
        };
        let once = fix(89.999, 179.999).at(Precision::City);
        assert_eq!((once.lat, once.lon), (90.0, 180.0));
        assert_eq!(
            once.at(Precision::City),
            once,
            "rounding twice changes nothing"
        );
        let south = fix(-0.004, -0.006).at(Precision::City);
        assert_eq!(south.lat.abs(), 0.0);
        assert_eq!(south.lon, -0.01);
    }

    #[test]
    fn exact_keeps_the_fix_and_a_rounded_one_stays_rounded() {
        let exact = Fix {
            lat: SEA.0,
            lon: SEA.1,
            accuracy_m: 7,
            precision: Precision::Exact,
        };
        assert_eq!(exact.at(Precision::Exact), exact);
        let rounded = exact.at(Precision::City);
        assert_eq!(rounded.at(Precision::Exact), rounded);
    }

    #[test]
    fn a_fix_goes_and_comes_back() {
        let fix = Fix {
            lat: 1.23,
            lon: -20.99,
            accuracy_m: 1000,
            precision: Precision::City,
        };
        assert_eq!(Fix::from_body(&fix.body()), Some(fix));
    }

    #[test]
    fn a_body_that_is_not_a_place_is_no_fix() {
        for body in [
            json!({}),
            json!({"lat": 1.0}),
            json!({"lat": "1.0", "lon": 2.0}),
            json!({"lat": 90.5, "lon": 2.0}),
            json!({"lat": 1.0, "lon": -180.5}),
            json!({"lat": null, "lon": 2.0}),
            json!({"error": "off"}),
        ] {
            assert_eq!(Fix::from_body(&body), None, "{body}");
        }
    }

    #[test]
    fn the_accuracy_is_held_to_what_a_fix_may_claim() {
        let with = |accuracy: Value| {
            Fix::from_body(&json!({"lat": 1.0, "lon": 2.0, "accuracy": accuracy}))
                .unwrap()
                .accuracy_m
        };
        assert_eq!(with(json!(0)), 1);
        assert_eq!(with(json!(12.2)), 13);
        assert_eq!(with(json!(1e12)), WORST_ACCURACY_M);
        assert_eq!(with(json!(-5)), WORST_ACCURACY_M, "negative: unknown");
        assert_eq!(with(json!("x")), WORST_ACCURACY_M);
        let none = Fix::from_body(&json!({"lat": 1.0, "lon": 2.0})).unwrap();
        assert_eq!(none.accuracy_m, WORST_ACCURACY_M);
        assert_eq!(none.precision, Precision::City, "unsaid: the safe one");
    }

    #[test]
    fn debug_never_shows_where() {
        let fix = Fix {
            lat: SEA.0,
            lon: SEA.1,
            accuracy_m: 25,
            precision: Precision::Exact,
        };
        let shown = format!("{fix:?} {:?}", Some(fix));
        assert!(shown.contains("25"));
        for digits in ["1.23456", "20.98765", "1.23", "20.98"] {
            assert!(!shown.contains(digits), "{shown}");
        }
    }
}
