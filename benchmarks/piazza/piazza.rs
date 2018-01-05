extern crate distributary;
extern crate rand;

#[macro_use]
extern crate clap;

use distributary::{ControllerHandle, DataType, ReuseConfigType, ControllerBuilder, LocalAuthority};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::{thread, time};

#[macro_use]
mod populate;

use populate::{Populate, NANOS_PER_SEC};

pub struct Backend {
    g: ControllerHandle<LocalAuthority>,
}

impl Backend {
    pub fn new(partial: bool, shard: bool, reuse: &str) -> Backend {
        let mut cb = ControllerBuilder::default();
        cb.set_local_workers(2);
        let log = distributary::logger_pls();
        let blender_log = log.clone();

        if !partial {
            cb.disable_partial();
        }

        if shard {
            cb.enable_sharding(2);
        }

        cb.log_with(blender_log);

        let mut g = cb.build_local();

        match reuse.as_ref() {
            "finkelstein" => g.enable_reuse(ReuseConfigType::Finkelstein),
            "full" => g.enable_reuse(ReuseConfigType::Full),
            "noreuse" => g.enable_reuse(ReuseConfigType::NoReuse),
            "relaxed" => g.enable_reuse(ReuseConfigType::Relaxed),
            _ => panic!("reuse configuration not supported"),
        }

        Backend {
            g: g,
        }
    }

    fn login(&mut self, user_context: HashMap<String, DataType>) -> Result<(), String> {

        self.g.create_universe(user_context.clone());

        self.write_to_user_context(user_context);
        Ok(())
    }

    fn write_to_user_context(&mut self, uc: HashMap<String, DataType>) {
        let name = &format!("UserContext_{}", uc.get("id").unwrap());
        let r: Vec<DataType> = uc.values().cloned().collect();
        let ins = self.g.inputs();
        let mut mutator = self
            .g
            .get_mutator(ins[name])
            .unwrap();

        mutator.put(r).unwrap();
    }

    fn set_security_config(&mut self, config_file: &str) {
        use std::io::Read;
        let mut config = String::new();
        let mut cf = File::open(config_file).unwrap();
        cf.read_to_string(&mut config).unwrap();

        // Install recipe with policies
        self.g.set_security_config(config);
    }

    fn migrate(
        &mut self,
        schema_file: &str,
        query_file: Option<&str>,
    ) -> Result<(), String> {
        use std::io::Read;

        // Read schema file
        let mut sf = File::open(schema_file).unwrap();
        let mut s = String::new();
        sf.read_to_string(&mut s).unwrap();

        let mut rs = s.clone();
        s.clear();

        // Read query file
        match query_file {
            None => (),
            Some(qf) => {
                let mut qf = File::open(qf).unwrap();
                qf.read_to_string(&mut s).unwrap();
                rs.push_str("\n");
                rs.push_str(&s);
            }
        }

        // Install recipe
        self.g.install_recipe(rs).unwrap();

        Ok(())
    }

    fn size(&mut self) -> usize {
        let outs = self.g.outputs();
        outs.into_iter().fold(0, |acc, (_, ni)| {
            acc + self.g.get_getter(ni).unwrap().len()
        })
    }
}

fn make_user(id: i32) -> HashMap<String, DataType> {
    let mut user = HashMap::new();
    user.insert(String::from("id"), id.into());

    user
}

fn main() {
    use clap::{App, Arg};
    let args = App::new("piazza")
        .version("0.1")
        .about("Benchmarks Piazza-like application with security policies.")
        .arg(
            Arg::with_name("schema")
                .short("s")
                .required(true)
                .default_value("benchmarks/piazza/schema.sql")
                .help("Schema file for Piazza application"),
        )
        .arg(
            Arg::with_name("queries")
                .short("q")
                .required(true)
                .default_value("benchmarks/piazza/post-queries.sql")
                .help("Query file for Piazza application"),
        )
        .arg(
            Arg::with_name("policies")
                .long("policies")
                .required(true)
                .default_value("benchmarks/piazza/ta-policies.json")
                .help("Security policies file for Piazza application"),
        )
        .arg(
            Arg::with_name("graph")
                .short("g")
                .default_value("pgraph.gv")
                .help("File to dump application's soup graph, if set"),
        )
        .arg(
            Arg::with_name("info")
                .short("i")
                .takes_value(true)
                .help("Directory to dump runtime process info (doesn't work on OSX)"),
        )
        .arg(
            Arg::with_name("reuse")
                .long("reuse")
                .default_value("full")
                .possible_values(&["noreuse", "finkelstein", "relaxed", "full"])
                .help("Query reuse algorithm"),
        )
        .arg(
            Arg::with_name("shard")
                .long("shard")
                .help("Enable sharding"),
        )
        .arg(
            Arg::with_name("partial")
                .long("partial")
                .help("Enable partial materialization"),
        )
        .arg(
            Arg::with_name("populate")
                .long("populate")
                .help("Populate app with randomly generated data"),
        )
        .arg(
            Arg::with_name("nusers")
                .short("u")
                .default_value("1000")
                .help("Number of users in the db"),
        )
        .arg(
            Arg::with_name("nlogged")
                .short("l")
                .default_value("1000")
                .help("Number of logged users. Must be less or equal than the number of users in the db")
            )
        .arg(
            Arg::with_name("nclasses")
                .short("c")
                .default_value("100")
                .help("Number of classes in the db"),
        )
        .arg(
            Arg::with_name("nposts")
                .short("p")
                .default_value("100000")
                .help("Number of posts in the db"),
        )
        .arg(
            Arg::with_name("private")
                .long("private")
                .default_value("0.1")
                .help("Percentage of private posts"),
        )
        .get_matches();


    println!("Starting benchmark...");

    // Read arguments
    let sloc = args.value_of("schema").unwrap();
    let qloc = args.value_of("queries").unwrap();
    let ploc = args.value_of("policies").unwrap();
    let gloc = args.value_of("graph");
    let iloc = args.value_of("info");
    let partial = args.is_present("partial");
    let shard = args.is_present("shard");
    let reuse = args.value_of("reuse").unwrap();
    let populate = args.is_present("populate");
    let nusers = value_t_or_exit!(args, "nusers", i32);
    let nlogged = value_t_or_exit!(args, "nlogged", i32);
    let nclasses = value_t_or_exit!(args, "nclasses", i32);
    let nposts = value_t_or_exit!(args, "nposts", i32);
    let private = value_t_or_exit!(args, "private", f32);

    assert!(nlogged <= nusers, "nusers must be greater than nlogged");

    // Initiliaze backend application with some queries and policies
    println!("Initiliazing database schema...");
    let mut backend = Backend::new(partial, shard, reuse);
    backend.migrate(sloc, None).unwrap();

    backend.set_security_config(ploc);
    backend.migrate(sloc, Some(qloc)).unwrap();

    let mut p = Populate::new(nposts, nusers, nclasses, private);
    if populate {
        println!("Populating tables...");
        p.populate_tables(&mut backend);
    }

    println!("Finished writing! Sleeping for 2 seconds...");
    thread::sleep(time::Duration::from_millis(2000));

    // Login a user
    println!("Login in users...");
    for i in 0..nlogged {
        let start = time::Instant::now();
        backend.login(make_user(i)).is_ok();
        let dur = dur_to_fsec!(start.elapsed());
        println!(
            "Migration {} took {:.2}s!",
            i,
            dur,
        );

        if iloc.is_some() && i % 50 == 0 {
            use std::fs;
            let fname = format!("{}-{}", iloc.unwrap(), i);
            fs::copy("/proc/self/status", fname).unwrap();
        }
    }

    let nreaders = backend.g.outputs().len();
    let nkeys = backend.size();
    println!("{} rows in {} leaf views (avg: {})", nkeys, nreaders, nkeys as f32 / nreaders as f32);


    println!("Done with benchmark.");

    if gloc.is_some() {
        let graph_fname = gloc.unwrap();
        let mut gf = File::create(graph_fname).unwrap();
        assert!(write!(gf, "{}", backend.g.graphviz()).is_ok());
    }
}
