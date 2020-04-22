use crate::{
    Command, DrivingGoal, PersonID, Scheduler, SidewalkPOI, SidewalkSpot, TripEndpoint, TripLeg,
    TripManager, VehicleSpec, VehicleType, BIKE_LENGTH, MAX_CAR_LENGTH,
};
use abstutil::Timer;
use geom::{Speed, Time, EPSILON_DIST};
use map_model::{BuildingID, BusRouteID, BusStopID, Map, PathConstraints, PathRequest, Position};
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub enum TripSpec {
    // Can be used to spawn from a border or anywhere for interactive debugging.
    CarAppearing {
        start_pos: Position,
        goal: DrivingGoal,
        vehicle_spec: VehicleSpec,
        ped_speed: Speed,
    },
    UsingParkedCar {
        start_bldg: BuildingID,
        goal: DrivingGoal,
        ped_speed: Speed,
    },
    JustWalking {
        start: SidewalkSpot,
        goal: SidewalkSpot,
        ped_speed: Speed,
    },
    UsingBike {
        start: SidewalkSpot,
        goal: DrivingGoal,
        vehicle: VehicleSpec,
        ped_speed: Speed,
    },
    UsingTransit {
        start: SidewalkSpot,
        goal: SidewalkSpot,
        route: BusRouteID,
        stop1: BusStopID,
        stop2: BusStopID,
        ped_speed: Speed,
    },
}

// This structure is created temporarily by a Scenario or to interactively spawn agents.
pub struct TripSpawner {
    trips: Vec<(PersonID, Time, TripSpec)>,
}

impl TripSpawner {
    pub fn new() -> TripSpawner {
        TripSpawner { trips: Vec::new() }
    }

    pub fn schedule_trip(&mut self, person: PersonID, start_time: Time, spec: TripSpec, map: &Map) {
        // TODO We'll want to repeat this validation when we spawn stuff later for a second leg...
        match &spec {
            TripSpec::CarAppearing {
                start_pos,
                vehicle_spec,
                goal,
                ..
            } => {
                if start_pos.dist_along() < vehicle_spec.length {
                    panic!(
                        "Can't spawn a {:?} at {}; too close to the start",
                        vehicle_spec.vehicle_type,
                        start_pos.dist_along()
                    );
                }
                if start_pos.dist_along() >= map.get_l(start_pos.lane()).length() {
                    panic!(
                        "Can't spawn a {:?} at {}; {} isn't that long",
                        vehicle_spec.vehicle_type,
                        start_pos.dist_along(),
                        start_pos.lane()
                    );
                }
                match goal {
                    DrivingGoal::Border(_, end_lane) => {
                        if start_pos.lane() == *end_lane
                            && start_pos.dist_along() == map.get_l(*end_lane).length()
                        {
                            panic!(
                                "Can't start a {:?} at the edge of a border already",
                                vehicle_spec.vehicle_type
                            );
                        }
                    }
                    DrivingGoal::ParkNear(_) => {}
                }
            }
            TripSpec::UsingParkedCar { .. } => {}
            TripSpec::JustWalking { start, goal, .. } => {
                if start == goal {
                    panic!(
                        "A trip just walking from {:?} to {:?} doesn't make sense",
                        start, goal
                    );
                }
            }
            TripSpec::UsingBike {
                start,
                goal,
                ped_speed,
                ..
            } => {
                // TODO These trips are just silently erased; they don't even show up as aborted
                // trips! Really need to fix the underlying problem.
                if SidewalkSpot::bike_from_bike_rack(start.sidewalk_pos.lane(), map).is_none() {
                    println!(
                        "Can't start biking from {}; no biking or driving lane nearby?",
                        start.sidewalk_pos.lane()
                    );
                    return;
                }
                if let DrivingGoal::ParkNear(b) = goal {
                    let last_lane = goal.goal_pos(PathConstraints::Bike, map).lane();
                    // If bike_to_sidewalk works, then SidewalkSpot::bike_rack should too.
                    if map
                        .get_parent(last_lane)
                        .bike_to_sidewalk(last_lane)
                        .is_none()
                    {
                        println!(
                            "Can't fulfill {:?} for a bike trip; no sidewalk near {}",
                            goal, last_lane
                        );
                        return;
                    }
                    // A bike trip going from one lane to the same lane should... just walk.
                    if start.sidewalk_pos.lane() == map.get_b(*b).sidewalk() {
                        println!(
                            "Bike trip from {:?} to {:?} will just walk; it's the same sidewalk!",
                            start, goal
                        );
                        self.trips.push((
                            person,
                            start_time,
                            TripSpec::JustWalking {
                                start: start.clone(),
                                goal: SidewalkSpot::building(*b, map),
                                ped_speed: *ped_speed,
                            },
                        ));
                        return;
                    }
                }
            }
            TripSpec::UsingTransit { .. } => {}
        };

        self.trips.push((person, start_time, spec));
    }

    pub fn finalize(
        mut self,
        map: &Map,
        trips: &mut TripManager,
        scheduler: &mut Scheduler,
        timer: &mut Timer,
        _retry_if_no_room: bool,
    ) {
        let paths = timer.parallelize(
            "calculate paths",
            std::mem::replace(&mut self.trips, Vec::new()),
            |tuple| {
                let req = tuple.2.get_pathfinding_request(map);
                (tuple, req.clone(), req.and_then(|r| map.pathfind(r)))
            },
        );

        timer.start_iter("spawn trips", paths.len());
        for ((p, start_time, spec), maybe_req, maybe_path) in paths {
            timer.next();

            // TODO clone() is super weird to do here, but we just need to make the borrow checker
            // happy. All we're doing is grabbing IDs off this.
            let person = trips.get_person(p).unwrap().clone();
            // Just create the trip for each case.
            // TODO Not happy about this clone()
            let trip = match spec.clone() {
                TripSpec::CarAppearing {
                    start_pos,
                    vehicle_spec,
                    goal,
                    ped_speed,
                } => {
                    let vehicle = if vehicle_spec.vehicle_type == VehicleType::Car {
                        vehicle_spec.make(person.car.unwrap(), Some(person.id))
                    } else {
                        vehicle_spec.make(person.bike.unwrap(), Some(person.id))
                    };
                    let mut legs = vec![TripLeg::Drive(vehicle.clone(), goal.clone())];
                    if let DrivingGoal::ParkNear(b) = goal {
                        legs.push(TripLeg::Walk(
                            person.ped,
                            ped_speed,
                            SidewalkSpot::building(b, map),
                        ));
                    }
                    let trip_start = TripEndpoint::Border(map.get_l(start_pos.lane()).src_i);
                    trips.new_trip(person.id, start_time, trip_start, legs)
                }
                TripSpec::UsingParkedCar {
                    start_bldg,
                    goal,
                    ped_speed,
                } => {
                    let walk_to = SidewalkSpot::deferred_parking_spot(start_bldg, goal, map);
                    // Can't add TripLeg::Drive, because we don't know the vehicle yet! Plumb along
                    // the DrivingGoal, so we can expand the trip later.
                    let legs = vec![TripLeg::Walk(person.ped, ped_speed, walk_to.clone())];
                    trips.new_trip(person.id, start_time, TripEndpoint::Bldg(start_bldg), legs)
                }
                TripSpec::JustWalking {
                    start,
                    goal,
                    ped_speed,
                } => trips.new_trip(
                    person.id,
                    start_time,
                    match start.connection {
                        SidewalkPOI::Building(b) => TripEndpoint::Bldg(b),
                        SidewalkPOI::SuddenlyAppear => {
                            TripEndpoint::Border(map.get_l(start.sidewalk_pos.lane()).src_i)
                        }
                        SidewalkPOI::Border(i) => TripEndpoint::Border(i),
                        _ => unreachable!(),
                    },
                    vec![TripLeg::Walk(person.ped, ped_speed, goal.clone())],
                ),
                TripSpec::UsingBike {
                    start,
                    vehicle,
                    goal,
                    ped_speed,
                } => {
                    let walk_to =
                        SidewalkSpot::bike_from_bike_rack(start.sidewalk_pos.lane(), map).unwrap();
                    let mut legs = vec![
                        TripLeg::Walk(person.ped, ped_speed, walk_to.clone()),
                        TripLeg::Drive(vehicle.make(person.bike.unwrap(), None), goal.clone()),
                    ];
                    match goal {
                        DrivingGoal::ParkNear(b) => {
                            legs.push(TripLeg::Walk(
                                person.ped,
                                ped_speed,
                                SidewalkSpot::building(b, map),
                            ));
                        }
                        DrivingGoal::Border(_, _) => {}
                    };
                    trips.new_trip(
                        person.id,
                        start_time,
                        match start.connection {
                            SidewalkPOI::Building(b) => TripEndpoint::Bldg(b),
                            SidewalkPOI::SuddenlyAppear => {
                                TripEndpoint::Border(map.get_l(start.sidewalk_pos.lane()).src_i)
                            }
                            SidewalkPOI::Border(i) => TripEndpoint::Border(i),
                            _ => unreachable!(),
                        },
                        legs,
                    )
                }
                TripSpec::UsingTransit {
                    start,
                    route,
                    stop1,
                    stop2,
                    goal,
                    ped_speed,
                } => {
                    let walk_to = SidewalkSpot::bus_stop(stop1, map);
                    trips.new_trip(
                        person.id,
                        start_time,
                        match start.connection {
                            SidewalkPOI::Building(b) => TripEndpoint::Bldg(b),
                            SidewalkPOI::SuddenlyAppear => {
                                TripEndpoint::Border(map.get_l(start.sidewalk_pos.lane()).src_i)
                            }
                            SidewalkPOI::Border(i) => TripEndpoint::Border(i),
                            _ => unreachable!(),
                        },
                        vec![
                            TripLeg::Walk(person.ped, ped_speed, walk_to.clone()),
                            TripLeg::RideBus(person.ped, route, stop2),
                            TripLeg::Walk(person.ped, ped_speed, goal),
                        ],
                    )
                }
            };
            scheduler.push(
                start_time,
                Command::StartTrip(trip, spec, maybe_req, maybe_path),
            );
        }
    }
}

impl TripSpec {
    // If possible, fixes problems that schedule_trip would hit.
    pub fn spawn_vehicle_at(pos: Position, is_bike: bool, map: &Map) -> Option<Position> {
        let lane_len = map.get_l(pos.lane()).length();
        let vehicle_len = if is_bike { BIKE_LENGTH } else { MAX_CAR_LENGTH };
        // There's no hope.
        if lane_len <= vehicle_len {
            return None;
        }

        if pos.dist_along() < vehicle_len {
            Some(Position::new(pos.lane(), vehicle_len))
        } else if pos.dist_along() == lane_len {
            Some(Position::new(pos.lane(), pos.dist_along() - EPSILON_DIST))
        } else {
            Some(pos)
        }
    }

    pub(crate) fn get_pathfinding_request(&self, map: &Map) -> Option<PathRequest> {
        match self {
            TripSpec::CarAppearing {
                start_pos,
                vehicle_spec,
                goal,
                ..
            } => {
                let constraints = vehicle_spec.vehicle_type.to_constraints();
                Some(PathRequest {
                    start: *start_pos,
                    end: goal.goal_pos(constraints, map),
                    constraints,
                })
            }
            // We don't know where the parked car will be
            TripSpec::UsingParkedCar { .. } => None,
            TripSpec::JustWalking { start, goal, .. } => Some(PathRequest {
                start: start.sidewalk_pos,
                end: goal.sidewalk_pos,
                constraints: PathConstraints::Pedestrian,
            }),
            TripSpec::UsingBike { start, .. } => Some(PathRequest {
                start: start.sidewalk_pos,
                end: SidewalkSpot::bike_from_bike_rack(start.sidewalk_pos.lane(), map)
                    .unwrap()
                    .sidewalk_pos,
                constraints: PathConstraints::Pedestrian,
            }),
            TripSpec::UsingTransit { start, stop1, .. } => Some(PathRequest {
                start: start.sidewalk_pos,
                end: SidewalkSpot::bus_stop(*stop1, map).sidewalk_pos,
                constraints: PathConstraints::Pedestrian,
            }),
        }
    }
}
